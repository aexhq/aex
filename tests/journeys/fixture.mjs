import assert from 'node:assert/strict';
import { spawn,execFileSync } from 'node:child_process';
import { once } from 'node:events';
import { createServer } from 'node:http';
import { randomBytes,randomUUID,createHash } from 'node:crypto';
import { mkdtemp,readFile,writeFile,rm,cp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join,resolve,relative } from 'node:path';
import { pathToFileURL } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import { database } from './database.mjs';

assert.ok(process.env.AEX_TEST_SDK_ENTRY,'the installed SDK consumer is required');
export const sdk=await import(pathToFileURL(process.env.AEX_TEST_SDK_ENTRY).href);
export async function until(read,accept=Boolean,label='condition',ms=30000) {const end=Date.now()+ms;let last;while(Date.now()<end){last=await read();if(accept(last))return last;await delay(30);}assert.fail(`timed out waiting for ${label}: ${JSON.stringify(last)}`);}
export async function listen(handler) {const server=createServer(handler);server.listen(0,'127.0.0.1');await once(server,'listening');return {server,url:`http://127.0.0.1:${server.address().port}`,close:()=>new Promise(resolve=>{server.closeAllConnections();server.close(resolve);})};}
async function address(){const listener=await listen(()=>{});const url=listener.url;await listener.close();return url;}
export async function body(req){const chunks=[];for await(const chunk of req)chunks.push(chunk);return JSON.parse(Buffer.concat(chunks));}

export function respond(res,answer,stream=true) {
  const usage={input_tokens:12,output_tokens:3,total_tokens:15,input_tokens_details:{cached_tokens:0},output_tokens_details:{reasoning_tokens:0}};
  const response={id:`resp_${randomUUID()}`,object:'response',created_at:1,status:'in_progress',model:'gpt-4.1-mini',output:[],usage:null};
  let sequence=0;const send=(type,data)=>res.write(`event: ${type}\ndata: ${JSON.stringify({type,sequence_number:sequence++,...data})}\n\n`);
  let item=typeof answer==='string'?{id:`msg_${randomUUID()}`,type:'message',role:'assistant',status:'completed',content:[{type:'output_text',text:answer,annotations:[]}]}:{id:`fc_${randomUUID()}`,type:'function_call',call_id:`call_${randomUUID()}`,name:answer.name,arguments:JSON.stringify(answer.args),status:'completed'};
  if(!stream){res.writeHead(200,{'content-type':'application/json'});return res.end(JSON.stringify({...response,status:'completed',output:[item],usage}));}
  res.writeHead(200,{'content-type':'text/event-stream'});send('response.created',{response});
  send('response.output_item.added',{output_index:0,item:{...item,status:'in_progress',...(item.type==='message'?{content:[]}:{arguments:''})}});
  if(item.type==='message'){
    send('response.content_part.added',{output_index:0,item_id:item.id,content_index:0,part:{type:'output_text',text:'',annotations:[]}});
    send('response.output_text.delta',{output_index:0,item_id:item.id,content_index:0,delta:answer});
    send('response.output_text.done',{output_index:0,item_id:item.id,content_index:0,text:answer});
    send('response.content_part.done',{output_index:0,item_id:item.id,content_index:0,part:item.content[0]});
  }else {send('response.function_call_arguments.delta',{output_index:0,item_id:item.id,delta:item.arguments});send('response.function_call_arguments.done',{output_index:0,item_id:item.id,arguments:item.arguments});}
  send('response.output_item.done',{output_index:0,item});send('response.completed',{response:{...response,status:'completed',output:[item],usage}});res.end();
}
function nativeEnvironment(extra={}) {
  const env={};for(const name of ['PATH','SystemRoot','SYSTEMROOT','TEMP','TMP','HOME','USERPROFILE','LOCALAPPDATA','APPDATA','NODE_EXTRA_CA_CERTS'])if(process.env[name])env[name]=process.env[name];return {...env,...extra};
}
function child(binary,args,env) {
  const process=spawn(binary,args,{env:nativeEnvironment(env),stdio:['ignore','pipe','pipe']});let logs='';process.stdout.on('data',data=>logs+=data);process.stderr.on('data',data=>logs+=data);
  const ended=once(process,'exit');let finished=false;ended.then(()=>{finished=true;});
  return {process,get logs(){return logs;},get finished(){return finished;},async stop(){if(!finished){process.kill();await ended;}}};
}

export async function fixture(t,setup) {
  for(const name of ['AEX_TEST_SERVER','AEX_TEST_RUNTIME','AEX_TEST_RUNNERS'])assert.ok(process.env[name],`${name} required`);
  const db=await database(),directory=await mkdtemp(join(tmpdir(),'aex-native-journey-'));
  const core=await address(),url=await address(),operator=await address();
  const tokens={brain:randomUUID(),operator:randomUUID(),site:randomUUID(),tools:randomUUID(),sign:randomUUID(),key:randomBytes(32).toString('base64')};
  const model={requests:[],keys:[],answers:[],delay:0};
  const provider=await listen(async(req,res)=>{
    assert.equal(req.url,'/v1/responses');const input=await body(req);model.requests.push(input);model.keys.push(req.headers.authorization);
    if(model.delay)await delay(model.delay);
    const last=input.input.at(-1);const text=typeof last.content==='string'?last.content:(last.content??[]).filter(part=>['input_text','text'].includes(part.type)).map(part=>part.text).join('\n');
    const call=last.type!=='function_call_output' && /CALL (\w+) (\{[^\n]*\})/.exec(text);
    const answer=model.answers.length?model.answers.shift():call?{name:call[1],args:JSON.parse(call[2])}:last.type==='function_call_output'?'tool handled':`answer: ${text}`;
    respond(res,answer,input.stream!==false);
  });
  const config=JSON.parse(await readFile('examples/config.json','utf8'));
  Object.assign(config,{listen:new URL(url).host,operator_listen:new URL(operator).host,data_dir:join(directory,'product'),brain_url:core,managed_providers:{session:{public:true,operations:['inspect','history'],active_per_account:100,profiles:{}}},billing:{pricebook:{id:'journey-tokens',rates:{model_tokens:{micro_usd:1,units:3},sandbox_ms:{micro_usd:1,units:1},attachment_byte_secs:{micro_usd:0,units:1},egress_bytes:{micro_usd:0,units:1}}},max_turn_secs:120,payments:null}});
  config.limits.minimum_free_disk_bytes=1;
  const runners=process.env.AEX_TEST_RUNNERS;
  const runtime={listen:new URL(core).host,public_url:url,data_dir:join(directory,'brain'),tokens:[],harnesses:{
    pi:{command:[process.execPath,join(runners,'pi/dist/main.js')]},codex:{command:[process.execPath,join(runners,'codex/src/main.mjs')],credential_hosts:true},opencode:{command:[process.execPath,join(runners,'opencode/src/main.mjs')]},
  },providers:{openai:{base_url:provider.url+'/v1',api:'openai-responses'}},credential_keys:{active:'journey',keys:{journey:'AEX_TEST_CREDENTIAL_KEY'}},tools:[],operator_url:operator,managed_providers:{session:{url:'',tools:[]}},persistence:{provider:'filesystem',directory:join(directory,'archive'),interval_seconds:3600,queue:32}};
  const services=await setup?.({config,runtime,tokens,url,operator,directory});
  await writeFile(join(directory,'aex.json'),JSON.stringify(config));await writeFile(join(directory,'brain.json'),JSON.stringify(runtime));
  let gateway,brain;const clients=[];
  async function operate(operation){const response=await fetch(operator+'/operate',{method:'POST',headers:{authorization:`Bearer ${tokens.operator}`,'content-type':'application/json'},body:JSON.stringify(operation)});const value=await response.json();assert.ok(response.ok,JSON.stringify(value));return value;}
  const f={db,directory,model,url,core,operator,config,runtime,tokens,operate,services,
    get logs(){return [gateway?.logs,brain?.logs].join('\n');},
    async start(){
      brain=child(process.env.AEX_TEST_RUNTIME,['-config',join(directory,'brain.json')],{AEX_BRAIN_TOKEN:tokens.brain,AEX_ENVIRONMENT_TOKEN:tokens.tools,AEX_TOOL_TOKEN:tokens.sign,AEX_TEST_CREDENTIAL_KEY:tokens.key});
      gateway=child(process.env.AEX_TEST_SERVER,['serve','--config',join(directory,'aex.json')],{...services?.gatewayEnv,AEX_DATABASE_URL:db.url,AEX_BRAIN_TOKEN:tokens.brain,AEX_OPERATOR_TOKEN:tokens.operator,AEX_SITE_TOKEN:tokens.site,AEX_ENVIRONMENT_TOKEN:tokens.tools});
      await until(async()=>{assert.ok(!brain.finished && !gateway.finished,f.logs);try{return (await fetch(url+'/health/live')).status===204 && (await fetch(core+'/health/ready')).status===200;}catch{return false;}},Boolean,'owning servers');
      await f.meter();await operate({action:'resume'});
    },
    async stop(){if(gateway && !gateway.finished)await operate({action:'drain'});await gateway?.stop();await brain?.stop();},
    async meter(){const rows=(await db.client.query("SELECT id,retained_bytes FROM sessions WHERE runtime='brain' AND state!='deleted'")).rows;await operate({action:'report_usage',observed_at:Math.floor(Date.now()/1000),sessions:Object.fromEntries(rows.map(row=>[row.id,Number(row.retained_bytes)]))});},
    async account(){
      const {account}=await operate({action:'create_account'}),issued=await operate({action:'issue_key',account});
      const accountToken=randomUUID();await db.client.query('INSERT INTO dashboard_sessions(verifier,account,expires) VALUES($1,$2,$3)',[createHash('sha256').update(accountToken).digest('hex'),account,Math.floor(Date.now()/1000)+3600]);
      const management=new sdk.Aex({baseUrl:url,accountToken});clients.push(management);await management.billing.update({pricebook:'journey-tokens',spend_limit_micro_usd:1000000});await operate({action:'adjust_credits',account,reference:randomUUID(),delta_micro_usd:1000000,reason:'ephemeral journey funding'});
      const api=new sdk.Aex({baseUrl:url,apiKey:issued.token,maxCostMicroUsd:1000000});clients.push(api);return {account,key:issued.key,token:issued.token,api,management,modelKey:'model-'+randomUUID()};
    },
    client(options){const client=new sdk.Aex({baseUrl:url,...options});clients.push(client);return client;},
    async coreRequest(account,path){return fetch(core+path,{method:'POST',headers:{authorization:`Bearer ${tokens.brain}`,'x-aex-tenant':account}});},
    async backup(){await f.stop();const rows=await db.snapshot();const backup=join(directory,'paired-backup');await cp(runtime.data_dir,backup,{recursive:true});return {rows,backup};},
    async restore(backup){await db.restore(backup.rows);assert.ok(relative(directory,runtime.data_dir)==='brain');await rm(runtime.data_dir,{recursive:true,force:true,maxRetries:10,retryDelay:100});await cp(backup.backup,runtime.data_dir,{recursive:true});await f.start();},
  };
  t.after(async()=>{for(const client of clients)await client.close().catch(()=>{});await f.stop();await provider.close();await db.close();assert.ok(relative(tmpdir(),directory).startsWith('aex-native-journey-'));await rm(directory,{recursive:true,force:true,maxRetries:10,retryDelay:100});});
  await f.start();return f;
}

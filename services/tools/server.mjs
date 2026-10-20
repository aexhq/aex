import { createServer } from 'node:http';
import { pathToFileURL } from 'node:url';
import { authenticate,controller,Journal,RemoteJournal,failure } from './common.mjs';
import { bounded } from '../media/server.mjs';
import { Sandboxes } from './sandbox.mjs';
import { Applications } from './application.mjs';
import { Browsers } from './browser.mjs';
import { modalProvider } from './modal.mjs';
import { setTimeout as delay } from 'node:timers/promises';

export function toolServer({token,control,providers,capacity=16}) {
  let active=0,paused=false;
  const server=createServer({maxHeaderSize:24576},async(req,res)=>{
    const send=(status,value)=>{if(!res.destroyed){res.writeHead(status,{'content-type':'application/json','cache-control':'no-store'});res.end(JSON.stringify(value));}};
    if(req.method==='GET' && req.url==='/health')return send(200,{status:'ok',paused,active});
    if(req.method==='POST' && ['/control/quiesce','/control/resume'].includes(req.url)) {
      if(req.headers.authorization!==`Bearer ${token}`)return send(401,{error:'unauthorized'});
      paused=req.url==='/control/quiesce';
      const end=Date.now()+300000;
      while(paused && active && Date.now()<end)await delay(25);
      return send(active && paused?409:200,{paused,active});
    }
    if(paused)return send(503,{error:'service_paused'});
    const callback=/^\/complete\/([a-f0-9]{64})$/.exec(req.url);
    if(req.method==='POST' && callback && providers.application) {
      active++;
      try {return send(200,await providers.application.complete(callback[1],JSON.parse(await bounded(req,2*1024*1024))));}
      catch {return send(409,{error:'completion_failed'});}
      finally {active--;}
    }
    let context;
    try {context=authenticate(req.headers.authorization,token);}catch{return send(401,{error:'unauthorized'});}
    const route=/^\/([a-z]+)\/([a-zA-Z][a-zA-Z0-9_]{0,63})$/.exec(req.url);
    if(req.method!=='POST' || !route || route[1]!==context.provider || !providers[context.provider])return send(404,{error:'not_found'});
    if(active>=capacity)return send(200,failure('tool service is at capacity; no action dispatched'));
    active++;
    const controller=new AbortController();res.on('close',()=>controller.abort());
    const timer=setTimeout(()=>controller.abort(),300000);
    try {
      const request=JSON.parse(await bounded(req,131072));
      if(request.tenant_id!==context.tenant || request.session_id!==context.session || typeof request.call_id!=='string' || !request.call_id || request.call_id.length>256 || !request.arguments || typeof request.arguments!=='object' || Array.isArray(request.arguments))return send(400,{error:'invalid_scope'});
      const {credential,...scope}=context;
      const admitted=await control('authorize',{...scope,action:true});
      res.writeHead(200,{'content-type':'text/event-stream','cache-control':'no-store'});res.flushHeaders();
      const value=await providers[context.provider].invoke(route[2],context,request,admitted,controller.signal,value=>{if(!res.destroyed)res.write(`event: progress\ndata: ${JSON.stringify(value)}\n\n`);});
      if(!res.destroyed)res.end(`event: result\ndata: ${JSON.stringify(value)}\n\n`);
    } catch {const value=failure('tool request failed; remote outcome may be unknown',true);if(res.headersSent){if(!res.destroyed)res.end(`event: result\ndata: ${JSON.stringify(value)}\n\n`);}else send(200,value);}
    finally {clearTimeout(timer);active--;}
  });
  server.maintain=async task=>{if(paused)return;active++;try{await task();}finally{active--;}};
  return server;
}

if(process.argv[1] && import.meta.url===pathToFileURL(process.argv[1]).href) {
  const token=process.env.AEX_TOOL_TOKEN;if(typeof token!=='string' || token.length<32)throw new Error('tool service credential required');
  const control=controller(process.env.AEX_OPERATOR_URL,process.env.AEX_ENVIRONMENT_TOKEN);
  const mode=process.env.AEX_TOOL_MODE;
  const journal=mode==='application'?new RemoteJournal(control):new Journal(process.env.AEX_TOOL_DATA_DIR),providers={};
  if(mode==='sandbox')providers.sandbox=new Sandboxes({journal,provider:modalProvider(),control});
  else if(mode==='application') {
    providers.application=new Applications({journal,callbackOrigin:process.env.AEX_PUBLIC_ORIGIN});
    if(process.env.AEX_BROWSER_URL)providers.browser=new Browsers({journal,url:process.env.AEX_BROWSER_URL,token:process.env.AEX_BROWSER_TOKEN,control});
  }
  else throw new Error('one installed tool service mode is required');
  const server=toolServer({token,control,providers});server.listen(Number(process.env.AEX_TOOL_PORT??8084),'127.0.0.1');
  let maintaining=Promise.resolve();
  const timer=setInterval(()=>{maintaining=maintaining.then(()=>server.maintain(async()=>{await providers.sandbox?.maintain();await providers.application?.maintain(control);await providers.browser?.maintain();})).catch(()=>console.error(JSON.stringify({event:'tool_maintenance_failed'})));},10000);
  for(const signal of ['SIGINT','SIGTERM'])process.once(signal,()=>{clearInterval(timer);server.close(()=>{maintaining.finally(()=>journal.close());});});
}

import { createServer } from 'node:http';
import { randomBytes, timingSafeEqual } from 'node:crypto';
import { pathToFileURL } from 'node:url';
import { maxBytes, renderPDF } from './render.mjs';

export function mediaServer({ token, attachmentOrigin, publicOrigin, render = renderPDF, fetcher = fetch, now = Date.now }) {
  if (typeof token !== 'string' || token.length < 32) throw new Error('media token required');
  const approved = new URL(attachmentOrigin);
  const pages = new Map(), deleted = new Map(), active = new Map(), uids = [11010,11011,11012,11013];
  let retained = 0;
  function expire() { for (const [key,page] of pages) if(page.until<=now()) {retained-=page.bytes.length;pages.delete(key);}for(const [scope,until] of deleted)if(until<=now())deleted.delete(scope); }
  const timer=setInterval(expire,30000);timer.unref();
  const server=createServer(async(req,res)=>{
    const send=(status,value)=>{res.writeHead(status,{'content-type':'application/json','cache-control':'no-store'});res.end(JSON.stringify(value));};
    if(req.method==='GET' && req.url==='/health') return send(200,{status:'ok'});
    expire();
    const page=/^\/pages\/([a-f0-9]{64})$/.exec(req.url);
    if(req.method==='GET' && page) {
      const item=pages.get(page[1]);if(!item)return send(404,{error:'not_found'});
      res.writeHead(200,{'content-type':'image/png','content-length':item.bytes.length,'cache-control':'private, no-store','x-content-type-options':'nosniff'});return res.end(item.bytes);
    }
    const expected=Buffer.from(`Bearer ${token}`),supplied=Buffer.from(req.headers.authorization??'');
    if(supplied.length!==expected.length || !timingSafeEqual(supplied,expected))return send(401,{error:'unauthorized'});
    if(req.method==='POST' && req.url==='/delete') {
      try {
        const input=JSON.parse(await bounded(req,8192)),scope=JSON.stringify([input.tenant,input.session]);deleted.set(scope,now()+900000);
        for(const [key,page] of pages)if(page.scope===scope){retained-=page.bytes.length;pages.delete(key);}
        for(const [controller,owner] of active)if(owner===scope)controller.abort();return send(200,{deleted:true});
      } catch {return send(400,{error:'invalid_request'});}
    }
    if(req.method!=='POST' || req.url!=='/render')return send(404,{error:'not_found'});
    const uid=uids.pop();if(uid===undefined || retained>236*1024*1024) {if(uid!==undefined)uids.push(uid);return send(503,{error:'capacity'});}
    const controller=new AbortController(),deadline=setTimeout(()=>controller.abort(),60000);
    res.on('close',()=>controller.abort());
    try {
      const input=JSON.parse(await bounded(req,8192));
      const scope=JSON.stringify([input.tenant,input.session]);if(deleted.has(scope))throw new Error('session media was deleted');active.set(controller,scope);
      const source=new URL(input.url);
      if(source.origin!==approved.origin || source.username || source.password || source.hash || !/^\/v1\/attachments\/att_[a-f0-9]+\/content$/.test(source.pathname) || !/^\?token=read_[a-f0-9]+$/.test(source.search))throw new Error('invalid attachment URL');
      const response=await fetcher(source,{signal:controller.signal,redirect:'error'});
      if(!response.ok || response.headers.get('content-type')!=='application/pdf')throw new Error('PDF unavailable');
      const rendered=await render(await bounded(response.body,maxBytes),uid,controller.signal);
      controller.signal.throwIfAborted();
      const references=rendered.map(bytes=>{
        const key=randomBytes(32).toString('hex');pages.set(key,{bytes,scope,until:now()+900000});retained+=bytes.length;
        return {type:'image',url:`${publicOrigin}/pages/${key}`};
      });
      send(200,references);
    } catch {if(!res.destroyed)send(400,{error:'conversion_failed'});}
    finally {clearTimeout(deadline);active.delete(controller);uids.push(uid);}
  });
  server.once('close',()=>clearInterval(timer));return server;
}
export async function bounded(stream,maximum) {
  const chunks=[];let length=0;
  for await(const chunk of stream) {length+=chunk.length;if(length>maximum)throw new Error('byte limit exceeded');chunks.push(chunk);}
  return Buffer.concat(chunks);
}
if(process.argv[1] && import.meta.url===pathToFileURL(process.argv[1]).href) {
  if(process.platform!=='linux' || process.getuid()!==0)throw new Error('media controller requires Linux isolation');
  const port=Number(process.env.AEX_MEDIA_PORT??8086),host='127.0.0.1';
  const server=mediaServer({token:process.env.AEX_MEDIA_TOKEN,attachmentOrigin:process.env.AEX_ATTACHMENT_ORIGIN,publicOrigin:`http://${host}:${port}`});
  server.listen(port,host);
  for(const signal of ['SIGINT','SIGTERM'])process.once(signal,()=>server.close());
}

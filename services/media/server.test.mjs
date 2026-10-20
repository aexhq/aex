import assert from 'node:assert/strict';
import test from 'node:test';
import { once } from 'node:events';
import { mediaServer } from './server.mjs';

const token='media-test-authority-0000000000000000';
test('PDF service admits only owned attachment destinations and expires opaque page capabilities',async t=>{
  let timestamp=1000,calls=0;
  const server=mediaServer({token,attachmentOrigin:'https://api.example',publicOrigin:'http://127.0.0.1:8086',now:()=>timestamp,
    fetcher:async(url,options)=>{calls++;assert.equal(url.origin,'https://api.example');assert.equal(options.redirect,'error');return new Response('%PDF-example', {headers:{'content-type':'application/pdf'}});},
    render:async(bytes,uid,signal)=>{assert.equal(bytes.toString(),'%PDF-example');assert.ok(uid>=11010 && uid<=11013);assert.ok(signal);return [Buffer.from('page-one'),Buffer.from('page-two')];}});
  server.listen(0,'127.0.0.1');await once(server,'listening');t.after(()=>new Promise(resolve=>server.close(resolve)));
  const base=`http://127.0.0.1:${server.address().port}`;
  const post=(url,authorized=true)=>fetch(`${base}/render`,{method:'POST',headers:{...(authorized?{authorization:`Bearer ${token}`}:{})},body:JSON.stringify({tenant:'a',session:'s',url})});
  const source='https://api.example/v1/attachments/att_abcd/content?token=read_abcd';
  assert.equal((await post(source,false)).status,401);
  for(const url of ['http://127.0.0.1/admin','https://api.example@evil.example/v1/attachments/att_abcd/content?token=read_abcd',source+'&other=1',source+'#fragment'])assert.equal((await post(url)).status,400);
  assert.equal(calls,0);
  const response=await post(source);assert.equal(response.status,200);const pages=await response.json();assert.equal(pages.length,2);
  const page=new URL(pages[0].url).pathname;
  assert.equal(await (await fetch(base+page)).text(),'page-one');
  timestamp+=900001;assert.equal((await fetch(base+page)).status,404);
});

test('PDF download limits and parser failures never publish pages',async t=>{
  const server=mediaServer({token,attachmentOrigin:'https://api.example',publicOrigin:'http://127.0.0.1:8086',fetcher:async()=>new Response('%PDF-bad',{headers:{'content-type':'application/pdf'}}),render:async()=>{throw new Error('malformed PDF');}});
  server.listen(0,'127.0.0.1');await once(server,'listening');t.after(()=>new Promise(resolve=>server.close(resolve)));
  const response=await fetch(`http://127.0.0.1:${server.address().port}/render`,{method:'POST',headers:{authorization:`Bearer ${token}`},body:JSON.stringify({url:'https://api.example/v1/attachments/att_abcd/content?token=read_abcd'})});
  assert.equal(response.status,400);assert.deepEqual(await response.json(),{error:'conversion_failed'});
});

test('session deletion revokes page URLs and prevents a late render from publishing again',async t=>{
  let enter,finish;const entered=new Promise(resolve=>enter=resolve),done=new Promise(resolve=>finish=resolve);
  const server=mediaServer({token,attachmentOrigin:'https://api.example',publicOrigin:'http://127.0.0.1:8086',fetcher:async()=>new Response('%PDF-fixture',{headers:{'content-type':'application/pdf'}}),render:async()=>{enter();await done;return [Buffer.from('private page')];}});
  server.listen(0,'127.0.0.1');await once(server,'listening');t.after(()=>new Promise(resolve=>server.close(resolve)));
  const base=`http://127.0.0.1:${server.address().port}`,post=(path,body)=>fetch(base+path,{method:'POST',headers:{authorization:`Bearer ${token}`},body:JSON.stringify(body)});
  const input={tenant:'a',session:'owned',url:'https://api.example/v1/attachments/att_abcd/content?token=read_abcd'};
  const pending=post('/render',input);await entered;assert.equal((await post('/delete',{tenant:'a',session:'owned'})).status,200);finish();assert.equal((await pending).status,400);assert.equal((await post('/render',input)).status,400);
});

import assert from 'node:assert/strict';
import test from 'node:test';
import { createServer } from 'node:https';
import { request } from 'node:http';
import { once } from 'node:events';
import { readFile } from 'node:fs/promises';
import { randomUUID } from 'node:crypto';
import { mediaServer } from '../../services/media/server.mjs';
import { renderPDF } from '../../services/media/render.mjs';
import { fixture,sdk } from './fixture.mjs';

// A small real PDF; offsets are generated so Poppler checks the normal file format.
function pdf(){const objects=['<< /Type /Catalog /Pages 2 0 R >>','<< /Type /Pages /Kids [3 0 R] /Count 1 >>','<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>','<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>'];const stream='BT /F1 16 Tf 20 50 Td (Aex PDF integration) Tj ET';objects.push(`<< /Length ${stream.length} >>\nstream\n${stream}\nendstream`);let value='%PDF-1.4\n',offsets=[0];for(const [index,object] of objects.entries()){offsets.push(Buffer.byteLength(value));value+=`${index+1} 0 obj\n${object}\nendobj\n`;}const xref=Buffer.byteLength(value);value+=`xref\n0 6\n0000000000 65535 f \n${offsets.slice(1).map(offset=>`${String(offset).padStart(10,'0')} 00000 n \n`).join('')}trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;return Buffer.from(value);}

test('real R2 attachments and isolated Poppler reach all native model adapters and are erased on deletion',{timeout:180000},async t=>{
  assert.equal(process.platform,'linux');assert.equal(process.getuid(),0,'PDF controller isolation requires root');
  for(const key of ['AEX_S3_ACCESS_KEY_ID','AEX_S3_SECRET_ACCESS_KEY','AEX_TEST_S3_BUCKET','AEX_TEST_S3_REGION','AEX_TEST_S3_ENDPOINT','NODE_EXTRA_CA_CERTS','AEX_TEST_TLS_KEY'])assert.ok(process.env[key],`${key} required`);
  const [png]=await renderPDF(pdf(),11013,AbortSignal.timeout(60000));
  const f=await fixture(t,async({config,runtime,url})=>{
    const proxy=createServer({cert:await readFile(process.env.NODE_EXTRA_CA_CERTS),key:await readFile(process.env.AEX_TEST_TLS_KEY)},(req,res)=>{const upstream=request(url+req.url,{method:req.method,headers:req.headers},reply=>{res.writeHead(reply.statusCode,reply.headers);reply.pipe(res);});upstream.on('error',()=>res.destroy());req.pipe(upstream);});
    proxy.listen(0,'127.0.0.1');await once(proxy,'listening');t.after(()=>new Promise(resolve=>{proxy.closeAllConnections();proxy.close(resolve);}));
    const origin=`https://127.0.0.1:${proxy.address().port}`,token=randomUUID();
    const media=mediaServer({token,attachmentOrigin:origin,publicOrigin:''});media.listen(0,'127.0.0.1');await once(media,'listening');
    // Published page URLs use the actual ephemeral listener, while the parser is real.
    const mediaOrigin=`http://127.0.0.1:${media.address().port}`;
    await new Promise(resolve=>media.close(resolve));
    const server=mediaServer({token,attachmentOrigin:origin,publicOrigin:mediaOrigin});server.listen(new URL(mediaOrigin).port,'127.0.0.1');await once(server,'listening');t.after(()=>new Promise(resolve=>{server.closeAllConnections();server.close(resolve);}));
    config.attachments={public_origin:origin,bucket:process.env.AEX_TEST_S3_BUCKET,region:process.env.AEX_TEST_S3_REGION,endpoint:process.env.AEX_TEST_S3_ENDPOINT,max_bytes:1048576,count_per_account:20,bytes_per_account:20971520,ttl_secs:600,transfer_timeout_secs:30,upload_origins:[]};config.media_url=mediaOrigin;runtime.media_origins=[origin,mediaOrigin];
    for(const harness of Object.values(runtime.harnesses))harness.command=['/usr/bin/env',`NODE_EXTRA_CA_CERTS=${process.env.NODE_EXTRA_CA_CERTS}`,...harness.command];
    return {gatewayEnv:{AEX_MEDIA_TOKEN:token,AEX_S3_ACCESS_KEY_ID:process.env.AEX_S3_ACCESS_KEY_ID,AEX_S3_SECRET_ACCESS_KEY:process.env.AEX_S3_SECRET_ACCESS_KEY}};
  });
  const a=await f.account(),b=await f.account();
  for(const harness of ['pi','codex','opencode']) {
    const session=await a.api.sessions.create({harness,model:sdk.Model('openai/gpt-4.1-mini',sdk.ApiKey(a.modelKey))});
    const image=await a.api.attachments.upload(session.id,png,{contentType:'image/png',idempotencyKey:randomUUID(),downloadBudgetBytes:1048576});
    const document=await a.api.attachments.upload(session.id,pdf(),{contentType:'application/pdf',idempotencyKey:randomUUID(),downloadBudgetBytes:1048576});
    const before=f.model.requests.length;
    try {assert.ok((await session.send({text:'Read the image and PDF',media:[image.media,document.media]})).output);}catch(error){assert.fail(`${harness}: ${error.message}\n${f.logs}`);}
    const recorded=JSON.stringify(f.model.requests.slice(before));assert.ok(recorded.includes('data:image/png;base64,'),harness);
    const images=f.model.requests.slice(before).flatMap(value=>value.input).flatMap(value=>value.content??[]).filter(value=>value.type==='input_image');assert.equal(images.length,2,harness);
    assert.equal((await fetch(image.media.url)).status,200);
    const other=await b.api.sessions.create({harness,model:sdk.Model('openai/gpt-4.1-mini',sdk.ApiKey(b.modelKey))});await assert.rejects(other.send({text:'stolen',media:[document.media]}),error=>error.status===404);await other.delete();
    await session.delete();assert.equal((await fetch(image.media.url)).status,404);assert.equal((await fetch(document.media.url)).status,404);
    const files=await f.db.client.query('SELECT state FROM attachments WHERE session=$1',[session.id]);assert.ok(files.rows.every(row=>row.state==='deleted'));
  }
});

import assert from 'node:assert/strict';
import test from 'node:test';
import { renderPDF } from './render.mjs';

function pdf(pages) {
  const objects=['<< /Type /Catalog /Pages 2 0 R >>',`<< /Type /Pages /Kids [${Array.from({length:pages},(_,i)=>`${3+i} 0 R`).join(' ')}] /Count ${pages} >>`];
  for(let i=0;i<pages;i++)objects.push('<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << >> >>');
  let output='%PDF-1.4\n';const offsets=[0];
  for(const [index,object] of objects.entries()){offsets.push(Buffer.byteLength(output));output+=`${index+1} 0 obj\n${object}\nendobj\n`;}
  const offset=Buffer.byteLength(output);output+=`xref\n0 ${objects.length+1}\n0000000000 65535 f \n${offsets.slice(1).map(value=>String(value).padStart(10,'0')+' 00000 n \n').join('')}trailer\n<< /Size ${objects.length+1} /Root 1 0 R >>\nstartxref\n${offset}\n%%EOF\n`;return Buffer.from(output);
}
test('isolated Linux Poppler renders real PDF pages and rejects malformed or excessive documents',async()=>{
  const pages=await renderPDF(pdf(2),11010,AbortSignal.timeout(60000));assert.equal(pages.length,2);
  for(const page of pages)assert.equal(page.subarray(1,4).toString(),'PNG');
  await assert.rejects(renderPDF(Buffer.from('%PDF-broken'),11010,AbortSignal.timeout(60000)));
  await assert.rejects(renderPDF(pdf(65),11010,AbortSignal.timeout(60000)),/page limit/);
  const controller=new AbortController();controller.abort();await assert.rejects(renderPDF(pdf(1),11010,controller.signal));
});
